use super::*;

/// The `Base` label with its fetch spinner, over the ref candidates.
fn base_section(draft: &CreateState, tight: gpui::Pixels) -> Div {
    let candidates = draft.base_candidates();
    let base_header = div()
        .flex()
        .items_center()
        .justify_between()
        .child(Text::label("Base"))
        .children(
            draft
                .fetching
                .then(|| SpinnerWithLabel::new("create-base-fetch", "fetching")),
        );
    let previous_base = draft.previous_base.clone();
    let base_list = FuzzyList::new(
        "create-base-list",
        candidates.iter().enumerate().map(|(index, candidate)| {
            let mut item = FuzzyItem::new(candidate.clone());
            if index == 0 {
                item = item.trailing("default");
            } else if previous_base.as_deref() == Some(candidate.as_str()) {
                item = item.trailing("(previous base)");
            }
            item
        }),
    )
    .cursor(draft.base_cursor)
    .cap(BASE_ROWS)
    .under_text_field(true)
    .harness_rows("dialog.row", 0)
    .empty(Text::ui("No base refs yet.").muted());

    div()
        .flex()
        .flex_col()
        .gap(tight)
        .child(base_header)
        .children(draft.base_error.as_ref().map(|error| {
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(Text::ui(error.clone()).tone(Tone::Danger).ellipsize())
                .child(KeyHintRow::new().key("enter", "retry"))
        }))
        .child(base_list)
}

/// The `Host` cycler, plus the one line that says why the shown host cannot take a create.
///
/// The cycler itself stays live even on a blocked host: the way out of an unreachable choice is
/// `\u{2190}` / `\u{2192}`, so locking the control would trap the user on it. What the blocked
/// entry loses is `Enter` (`can_submit`), and the reason says so in the daemon's own words.
pub(super) fn host_section(draft: &CreateState, tight: gpui::Pixels, cx: &App) -> Option<Div> {
    let choice = draft.selected_choice()?;
    let warning = Tone::Warning.color(cx.theme());
    let cycler = Cycler::labeled("Host", choice.label.clone())
        .has_prev(draft.host_index > 0)
        .has_next(draft.host_index + 1 < draft.hosts.len())
        .focused(draft.field == Field::Host);
    // The note is one line in every state, so cycling hosts never moves the rest of the dialog.
    let note = div().flex().items_center().gap(tight);
    let note = match choice.blocked.as_deref() {
        Some(reason) => note
            .child(Icon::CloudOff.el().size(IconSize::Small).color(warning))
            .child(
                Text::ui(format!("{} \u{2014} {reason}", choice.label))
                    .tone(Tone::Warning)
                    .ellipsize(),
            ),
        None => note.child(Text::hint(
            choice
                .provider
                .clone()
                .unwrap_or_else(|| "this machine".to_owned()),
        )),
    };
    Some(div().flex().flex_col().gap(tight).child(cycler).child(note))
}

/// The two lines under the fields: how long a create will take, and what runs after it.
fn expectation(draft: &CreateState, cx: &App) -> Div {
    let hair = cx.theme().space.xxs;
    // §3.8.1 Icons: `zap` and `hourglass`, both 16 px Lucide strokes. A colour emoji here was
    // the one glyph on the screen that was not part of the icon set (§0), and §1.4 keeps amber
    // for "in flight / needs attention" rather than for decoration.
    let (expectation_icon, expectation_tone, expectation) = if draft.prepared_ready {
        (
            Icon::Zap,
            Tone::Warning,
            "prepared copy ready \u{2014} create takes ~2 s",
        )
    } else {
        (
            Icon::Hourglass,
            Tone::Muted,
            "no prepared copy \u{2014} the first create copies the repo (~40 s) in the background",
        )
    };
    let hooks_line = if draft.hooks.is_empty() {
        "hooks: none".to_owned()
    } else {
        format!(
            "hooks: {}  (run in background)",
            draft.hooks.join(" \u{00b7} ")
        )
    };

    div()
        .flex()
        .flex_col()
        .gap(hair)
        .child(
            div()
                .flex()
                .items_center()
                .gap(hair)
                .child(
                    expectation_icon
                        .el()
                        .size(IconSize::Medium)
                        .color(expectation_tone.color(cx.theme())),
                )
                .child(Text::ui(expectation).muted()),
        )
        .child(Text::hint(hooks_line))
}

/// Renders the dialog (§3.8.1).
pub(crate) fn render(
    state: &Entity<AppState>,
    bridge: &Bridge,
    focus: &FocusHandle,
    host: &Entity<DialogHost>,
    _window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let (gap, tight) = {
        let theme = cx.theme();
        (theme.space.md, theme.space.xs)
    };
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
        .child(base_section(draft, tight).harness_target_indexed("dialog.field", 1))
        .children(
            host_section(draft, tight, cx)
                .map(|section| section.harness_target_indexed("dialog.field", 2)),
        )
        .child(expectation(draft, cx));

    let mut card = Dialog::new("New worktree")
        .dismiss_action(crate::dialogs::Dialogs::CreateWorktree.dismiss_action())
        .icon(Icon::GitBranchPlus)
        .width(crate::dialogs::Dialogs::CreateWorktree.width(cx))
        .body(body)
        .hint_row(
            KeyHintRow::new()
                .key("\u{21e5}", "field")
                .key("\u{2303}n/\u{2303}p", "base")
                .key("esc", "cancel"),
        )
        .primary(if duplicate.is_some() {
            "\u{23ce} Open"
        } else {
            "\u{23ce} Create"
        });
    if let Some(repo) = draft.repo.as_ref() {
        card = card.subtitle(format!("\u{00b7} {}", repo.as_str()));
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
            submit(true, &create_state, &create_bridge, cx);
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
                    state.toast_short(
                        "\u{27f3} base fetch still running \u{00b7} J",
                        Icon::LoaderCircle,
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
