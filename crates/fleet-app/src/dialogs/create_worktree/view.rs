use super::*;

/// "Start from": one box holding what narrows the list, the fetch state, and the refs.
///
/// The list is filtered by the branch being typed (§3.8.1), so the box's top row says what it is
/// matching rather than offering a second field to type into: the tab cycle stays branch, base,
/// host. A row click chooses that base; the check marks the chosen one.
fn base_section(
    draft: &CreateState,
    state: &Entity<AppState>,
    bridge: &Bridge,
    focus: &FocusHandle,
    cx: &App,
) -> Div {
    let theme = cx.theme();
    let (candidates, filtered) = draft.base_rows();
    let cursor = draft.base_cursor;
    let previous_base = draft.previous_base.as_deref();
    let has_default = !draft.default_base.is_empty();
    let items = candidates.iter().enumerate().map(|(index, candidate)| {
        let mut item = FuzzyItem::new(candidate.clone()).checked(index == cursor);
        if index == 0 && has_default {
            item = item.badge("default");
        } else if previous_base == Some(candidate.as_str()) {
            item = item.badge("previous base");
        }
        item
    });
    let click_state = state.clone();
    let click_focus = focus.clone();
    let base_list = FuzzyList::new("create-base-list", items)
        .cursor(cursor)
        .visible_rows(BASE_ROWS)
        .track_scroll(&draft.base_scroll)
        .under_text_field(true)
        .harness_rows("dialog.row", 0)
        .on_click(move |index, window, cx| {
            select_base(&click_state, index, &click_focus, window, cx);
        })
        .empty(Text::ui("No base refs yet.").muted());

    let matching = if filtered {
        Text::ui(format!("Matching \u{201c}{}\u{201d}", draft.branch))
            .muted()
            .ellipsize()
    } else if draft.branch.is_empty() {
        Text::ui("Type a branch name to narrow the list")
            .faint()
            .ellipsize()
    } else {
        Text::ui(format!(
            "Nothing matches \u{201c}{}\u{201d} \u{2014} every ref is listed",
            draft.branch
        ))
        .faint()
        .ellipsize()
    };
    let status = if draft.fetching {
        SpinnerWithLabel::new("create-base-fetch", "fetching").into_any_element()
    } else if let Some(error) = draft.base_error.as_ref() {
        let retry = Button::new("create-base-retry", "Retry")
            .style(ButtonStyle::Ghost)
            .size(ButtonSize::Compact);
        // `⏎` retries while the list has the keyboard (or nothing could be created anyway);
        // the button shows that key then, and otherwise just retries.
        let retry = if draft.should_retry_base_refs() {
            retry.action(Box::new(dialog::Confirm))
        } else {
            let state = state.clone();
            let bridge = bridge.clone();
            retry.on_click(move |_, _, cx| retry_base_refs(&state, &bridge, cx))
        };
        div()
            .flex()
            .min_w_0()
            .items_center()
            .gap(theme.space.sm)
            .child(Text::ui(error.clone()).tone(Tone::Danger).ellipsize())
            .child(retry)
            .into_any_element()
    } else {
        let count = draft.base_refs.len();
        Text::hint(format!(
            "{count} {}",
            if count == 1 { "ref" } else { "refs" }
        ))
        .faint()
        .into_any_element()
    };
    let filter_row = div()
        .flex()
        .flex_none()
        .items_center()
        .gap(theme.space.sm)
        .h(theme.metrics.row_h)
        .px(theme.space.md)
        .border_b(theme.metrics.hairline)
        .border_color(theme.colors.border)
        .child(
            Icon::Search
                .el()
                .size(IconSize::Small)
                .color(theme.colors.text_muted),
        )
        .child(div().flex().flex_1().min_w_0().child(matching))
        .child(div().flex().flex_none().child(status));

    let focused = draft.field == Field::Base;
    div()
        .flex()
        .flex_col()
        .gap(theme.space.xxs)
        .child(Text::label("Start from"))
        .child(
            div()
                .flex()
                .flex_col()
                .w_full()
                .rounded(theme.radii.sm)
                .bg(theme.colors.bg)
                .border(theme.metrics.hairline)
                .border_color(if focused {
                    theme.colors.focus_ring
                } else {
                    theme.colors.border
                })
                .overflow_hidden()
                .child(filter_row)
                .child(div().p(theme.space.xxs).child(base_list)),
        )
}

/// "Run on": the hosts side by side (a dropdown past four), or nothing on a local-only daemon,
/// with a line under it naming every host that cannot take a create and why.
///
/// The keys stay the cycler's: `←` / `→` still reach a blocked host, because the way out of an
/// unreachable choice is the next arrow and locking the control would trap the user on it. What
/// a blocked host loses is `Enter` (`can_submit`) and the click, and the note says why in the
/// daemon's own words.
pub(super) fn host_section(draft: &CreateState, state: &Entity<AppState>, cx: &App) -> Option<Div> {
    let choice = draft.selected_choice()?;
    let theme = cx.theme();
    let tight = theme.space.xs;
    let warning = Tone::Warning.color(theme);
    let blocked: Vec<usize> = draft
        .hosts
        .iter()
        .enumerate()
        .filter(|(_, host)| host.blocked.is_some())
        .map(|(index, _)| index)
        .collect();
    let click_state = state.clone();
    let cycler = Cycler::labeled("Run on", choice.label.clone())
        .id("create-host")
        .options(draft.hosts.iter().map(|host| host.label.clone()))
        .unavailable(blocked.iter().copied())
        .harness_segments("dialog.segment")
        .on_select(move |index, _, cx| select_host(&click_state, index, cx))
        .has_prev(draft.host_index > 0)
        .has_next(draft.host_index + 1 < draft.hosts.len())
        .focused(draft.field == Field::Host);
    let note = div().flex().items_center().gap(tight);
    let reasons = unavailable_hosts(draft);
    let note = match reasons.as_slice() {
        [] => note.child(Text::hint(
            choice
                .provider
                .clone()
                .unwrap_or_else(|| "this machine".to_owned()),
        )),
        reasons => note
            .child(Icon::CloudOff.el().size(IconSize::Small).color(warning))
            .child(
                Text::ui(reasons.join(" \u{00b7} "))
                    .tone(Tone::Warning)
                    .ellipsize(),
            ),
    };
    Some(div().flex().flex_col().gap(tight).child(cycler).child(note))
}

/// `devbox unavailable — unreachable` for every host that cannot take a create, the chosen one
/// first, so the line the eye reads first explains why `Enter` is refused.
pub(super) fn unavailable_hosts(draft: &CreateState) -> Vec<String> {
    let chosen = draft.host_index;
    let mut blocked: Vec<(usize, &HostChoice)> = draft
        .hosts
        .iter()
        .enumerate()
        .filter(|(_, host)| host.blocked.is_some())
        .collect();
    blocked.sort_by_key(|(index, _)| *index != chosen);
    blocked
        .into_iter()
        .map(|(_, host)| {
            format!(
                "{} unavailable \u{2014} {}",
                host.label,
                host.blocked.as_deref().unwrap_or_default()
            )
        })
        .collect()
}

/// How long a create will take, and what runs after it, before the user commits.
fn expectation(draft: &CreateState) -> Callout {
    // §3.8.1 Icons: `zap` and `hourglass`. Green is the fast path that is ready; amber says the
    // first create will be slow, which is worth knowing before switching away.
    let callout = if draft.prepared_ready {
        Callout::new(
            Tone::Success,
            Icon::Zap,
            "Prepared copy ready \u{2014} about 2 s",
        )
    } else {
        Callout::new(
            Tone::Warning,
            Icon::Hourglass,
            "No prepared copy \u{2014} the first create copies the repo (~40 s) in the background",
        )
    };
    callout.detail(if draft.hooks.is_empty() {
        "Hooks: none".to_owned()
    } else {
        format!(
            "Hooks: {} (run in background)",
            draft.hooks.join(" \u{00b7} ")
        )
    })
}

/// Renders the dialog (§3.8.1).
pub(crate) fn render(
    state: &Entity<AppState>,
    bridge: &Bridge,
    focus: &FocusHandle,
    host: &Entity<DialogHost>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let gap = cx.theme().space.lg;
    let draft = &host.read(cx).create;
    let duplicate = draft
        .preview_id()
        .filter(|id| existing_worktree(state.read(cx), id).is_some());
    let Some(branch) = host.read(cx).create_branch.clone() else {
        return root(focus).into_any_element();
    };

    // `dialog.field[N]` counts the tab cycle, so the indices are exactly [`Field`]'s order:
    // branch, base, host. The host cycler is absent on a single-host daemon, and its index is
    // simply absent from the dump with it.
    let body = div()
        .flex()
        .flex_col()
        .gap(gap)
        .child(branch.harness_target_indexed("dialog.field", 0))
        .child(
            base_section(draft, state, bridge, focus, cx).harness_target_indexed("dialog.field", 1),
        )
        .children(
            host_section(draft, state, cx)
                .map(|section| section.harness_target_indexed("dialog.field", 2)),
        )
        .child(expectation(draft));

    // The box decides what `Enter` does; `⌥Enter` always creates without opening, and the
    // box's tooltip says so with that key from the live keymap.
    let open_state = state.clone();
    let open_after = Checkbox::new(
        "create-open-after",
        "Open after creating",
        !draft.stay_in_hub,
    )
    .on_toggle(move |open, _, cx| set_open_after(&open_state, open, cx));
    let open_after = div()
        .id("create-open-after-tip")
        .child(open_after)
        .with_tooltip(
            Tooltip::new("Create without opening").kbd(Kbd::for_action(
                &create_actions::CreateWithoutOpening,
                window,
                cx,
            )),
            cx,
        )
        .harness_target("dialog.checkbox");
    let create = Button::new(
        "create-submit",
        if duplicate.is_some() {
            "Open"
        } else {
            "Create"
        },
    )
    .style(ButtonStyle::Primary)
    .action(Box::new(dialog::Confirm))
    .disabled(!draft.can_press_create());

    let mut card = Dialog::new("New worktree")
        .dismiss_action(crate::dialogs::Dialogs::CreateWorktree.dismiss_action())
        .icon(Icon::GitBranchPlus)
        .width(crate::dialogs::Dialogs::CreateWorktree.width(cx))
        .body(body)
        .footer_start(open_after)
        .actions(vec![
            Button::new("create-cancel", "Cancel").action(Box::new(dialog::Cancel)),
            create,
        ]);
    if let Some(repo) = draft.repo.as_ref() {
        card = card.subtitle(repo.as_str().to_owned());
    }
    if let Some(message) = draft.error.clone() {
        card = card.error(message);
    }

    let create_state = state.clone();
    let create_bridge = bridge.clone();
    let alt_state = state.clone();
    let alt_bridge = bridge.clone();
    let cancel_state = state.clone();

    // `left` / `right` cycle the host under browsing `Dialog > Create`; while the branch editor
    // owns the keyboard the dialog publishes `CreateEditing`, those rows leave the chain, and
    // the arrows move the caret through `FleetTextInput` instead.
    root(focus)
        .on_action({
            let state = state.clone();
            let focus = focus.clone();
            move |_: &dialog::NextField, window, cx| move_field(&state, 1, &focus, window, cx)
        })
        .on_action({
            let state = state.clone();
            let focus = focus.clone();
            move |_: &dialog::PrevField, window, cx| move_field(&state, -1, &focus, window, cx)
        })
        .on_action({
            let state = state.clone();
            let focus = focus.clone();
            move |_: &dialog::CursorDown, window, cx| move_base(&state, 1, &focus, window, cx)
        })
        .on_action({
            let state = state.clone();
            let focus = focus.clone();
            move |_: &dialog::CursorUp, window, cx| move_base(&state, -1, &focus, window, cx)
        })
        .on_action({
            let state = state.clone();
            let focus = focus.clone();
            move |_: &create_actions::HostPrev, window, cx| {
                cycle_host(&state, -1, &focus, window, cx);
            }
        })
        .on_action({
            let state = state.clone();
            let focus = focus.clone();
            move |_: &create_actions::HostNext, window, cx| {
                cycle_host(&state, 1, &focus, window, cx);
            }
        })
        .on_action(move |_: &dialog::Confirm, _window, cx| {
            let open_after = !with_host(&create_state, cx, |host| host.create.stay_in_hub);
            submit(open_after, &create_state, &create_bridge, cx);
        })
        .on_action(
            move |_: &create_actions::CreateWithoutOpening, _window, cx| {
                submit(false, &alt_state, &alt_bridge, cx);
            },
        )
        .on_action(move |_: &dialog::Cancel, _window, cx| {
            // §3.8.1: closing the dialog never cancels a running base fetch; it says so.
            if with_host(&cancel_state, cx, |host| host.create.fetching) {
                cancel_state.update(cx, |state, cx| {
                    state.toast_short_to(
                        "Base fetch still running",
                        Icon::LoaderCircle,
                        crate::state::ToastTarget::Jobs,
                        Instant::now(),
                    );
                    cx.notify();
                });
            }
            // The shell owns closing the dialog.
            cx.propagate();
        })
        .child(card)
        .into_any_element()
}
