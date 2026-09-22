use super::*;

/// Loads the target card's current value for the kind the opener staged on the draft.
pub(crate) fn seed(state: &Entity<AppState>, cx: &mut App) {
    let target = read_host(state, cx, |host, _| host.card_picker.card_id.clone());
    let card = state
        .read(cx)
        .board()
        .and_then(|view| {
            target
                .as_ref()
                .and_then(|id| view.cards.iter().find(|card| &card.id == id))
        })
        .cloned();
    let kind_now = read_host(state, cx, |host, _| host.card_picker.kind.clone());
    // The cursor opens on the value the card already holds. Leaving it at `0` makes `Enter` on a
    // picker the user opened only to look at a write: `s` would move the card to the board's
    // first column — a transition pushed to the real issue on a linked board — and `p` would set
    // `Urgent`, whatever the card held.
    // A kind whose ladder does not contain the card's value — every due date, an estimate off
    // the 0/1/2/3/5/8/13 rungs — found no row and left the cursor on the clear row, so `Enter`
    // on a picker opened only to look wiped the field: exactly the failure this seed exists to
    // prevent. Its own value goes into the query instead, where `candidates` offers it back as
    // the "use this" row at index 0.
    let current = card
        .as_ref()
        .and_then(|card| current_value(card, &kind_now))
        .filter(|value| !value.is_empty());
    let at = current.as_ref().and_then(|current| {
        options(
            state.read(cx),
            &kind_now,
            card.as_ref().map(|card| &card.id),
        )
        .iter()
        .position(|option| option.value == *current)
    });
    let schema = property_kind(state.read(cx), &kind_now);
    let query = match (&at, &current) {
        (None, Some(current)) if accepts_free_text(&kind_now, schema) => current.clone(),
        _ => String::new(),
    };
    let input = cx.new(|cx| {
        let mut input = TextInput::new(InputMode::SingleLine, cx);
        input.set_filter(Some(|character| character != ' '), cx);
        input.set_label(Some(picker_label(state.read(cx), &kind_now).into()), cx);
        input.set_placeholder(
            if kind_now.is_multi_select(schema) {
                "filter values"
            } else {
                "type to filter or set"
            },
            cx,
        );
        input.set_text(query.clone(), cx);
        input.set_invalid(
            free_text_error(&kind_now, query.trim(), schema).map(Into::into),
            cx,
        );
        input
    });
    // The `Blocks` picker opens on the cards that wait for this one, which no card stores:
    // `ops::query::blocks` derives that direction from everyone else's blockers, and the
    // selection has to be seeded before the dialog takes the board's borrow.
    let dependants: Vec<String> = card.as_ref().map_or_else(Vec::new, |card| {
        state.read(cx).board().map_or_else(Vec::new, |view| {
            fleet_core::board::blocks(&view.cards, &card.id)
                .into_iter()
                .map(|other| other.id.as_str().to_owned())
                .collect()
        })
    });
    let host = crate::dialogs::host::host_for(state, cx);
    host.update(cx, |host, _| {
        let kind = host.card_picker.kind.clone();
        let then_worktree = host.card_picker.then_worktree;
        let then_detail = host.card_picker.then_detail;
        let selected = match (&kind, card.as_ref()) {
            (PickerKind::Labels, Some(card)) => card
                .labels
                .iter()
                .map(|id| id.as_str().to_owned())
                .collect(),
            (PickerKind::BlockedBy, Some(card)) => card
                .blocked_by
                .iter()
                .map(|id| id.as_str().to_owned())
                .collect(),
            (PickerKind::Blocks, Some(_)) => dependants.clone(),
            (PickerKind::Property(key), Some(card)) => match card.properties.get(key) {
                Some(PropertyValue::MultiSelect(values)) => values.clone(),
                _ => Vec::new(),
            },
            _ => Vec::new(),
        };
        host.card_picker = CardPickerState {
            kind,
            card_id: card.map(|card| card.id),
            selected,
            then_worktree,
            then_detail,
            cursor: at.unwrap_or(0),
            ..Default::default()
        };
        host.card_picker_input = Some(input.clone());
    });
    let weak_host = host.downgrade();
    let weak_state = state.downgrade();
    let subscription = cx.subscribe(&input, move |input, event, cx| {
        if !matches!(event, TextInputEvent::Changed) {
            return;
        }
        let Some(host) = weak_host.upgrade() else {
            return;
        };
        let Some(state) = weak_state.upgrade() else {
            return;
        };
        let query = input.read(cx).text().to_owned();
        let invalid = {
            let draft = &host.read(cx).card_picker;
            free_text_error(
                &draft.kind,
                query.trim(),
                property_kind(state.read(cx), &draft.kind),
            )
        };
        let input = input.clone();
        // The event originates inside this input's update. Validation belongs to the next update
        // turn so the subscriber never re-enters the entity that is emitting it.
        cx.defer(move |cx| {
            input.update(cx, |input, cx| {
                input.set_invalid(invalid.map(Into::into), cx)
            })
        });
        host.update(cx, |host, cx| {
            host.card_picker.cursor = 0;
            host.card_picker.error = None;
            prepare(state.read(cx), &mut host.card_picker, &query);
            cx.notify();
        });
    });
    host.update(cx, |host, _| {
        host.card_picker_input_subscription = Some(subscription)
    });
    let query = input.read(cx).text().to_owned();
    host.update(cx, |host, cx| {
        prepare(state.read(cx), &mut host.card_picker, &query);
    });
}

/// Refresh rows when the board or repository snapshot changes under the open picker.
pub(crate) fn refresh(state: &Entity<AppState>, cx: &mut App) {
    let query = read_host(state, cx, |host, cx| {
        host.card_picker_input
            .as_ref()
            .map_or_else(String::new, |input| input.read(cx).text().to_owned())
    });
    let host = crate::dialogs::host::host_for(state, cx);
    host.update(cx, |host, cx| {
        let before = host.card_picker.prepared.clone();
        prepare(state.read(cx), &mut host.card_picker, &query);
        if host.card_picker.prepared != before {
            host.card_picker.cursor = step(host.card_picker.cursor, 0, host.card_picker.rows.len());
            cx.notify();
        }
    });
}

/// `Enter`: turn the selection into a request, send it, and close.
pub(super) fn apply(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    let (draft, query) = read_host(state, cx, |host, cx| {
        (
            host.card_picker.clone(),
            host.card_picker_input
                .as_ref()
                .map_or_else(String::new, |input| input.read(cx).text().to_owned()),
        )
    });
    let Some(card_id) = draft.card_id.clone() else {
        return;
    };
    let card = state
        .read(cx)
        .board()
        .and_then(|view| view.cards.iter().find(|card| card.id == card_id).cloned());
    let Some(card) = card else {
        // A reload can drop the card out from under an open picker. Every other refusal in this
        // dialog says so; returning silently makes `Enter` a dead key with nothing to read.
        with_host(state, cx, |host| {
            host.card_picker.error = Some("That card is no longer on this board".into());
        });
        notify(state, cx);
        return;
    };
    let schema = property_kind(state.read(cx), &draft.kind);
    if let Some(message) = free_text_error(&draft.kind, query.trim(), schema) {
        with_host(state, cx, |host| host.card_picker.error = Some(message));
        notify(state, cx);
        return;
    }
    let row = prepared(state, cx).get(draft.cursor).cloned();
    // A multi-select applies `draft.selected`, not the row under the cursor: filtering the list
    // down to nothing after toggling still leaves a perfectly good set to send, and a board with
    // no labels at all would otherwise make `t` a picker that can never apply anything.
    let multi = draft.kind.is_multi_select(schema);
    // A listed row that cannot be taken refuses `Enter` the way it refuses `space`: the dialog
    // stays open with the row's own reason on the error line (contracts §5.3). A multi-select
    // never applies the row under the cursor, and `toggle` already kept it out of the set.
    if let Some(row) = row.as_ref().filter(|row| row.disabled && !multi) {
        let reason = row
            .detail
            .clone()
            .unwrap_or_else(|| "cannot be picked".to_owned());
        let message = format!("{} \u{2014} {reason}", row.label);
        with_host(state, cx, |host| host.card_picker.error = Some(message));
        notify(state, cx);
        return;
    }
    let chosen = row.map(|option| option.value);
    // A kind whose values are a fixed list takes no typed one: falling back to the query would
    // send a status or label the board does not have and wait for the daemon to say so.
    if chosen.is_none() && !multi && !accepts_free_text(&draft.kind, schema) {
        with_host(state, cx, |host| {
            host.card_picker.error = Some("No match \u{2014} pick a value".into());
        });
        notify(state, cx);
        return;
    }
    let value = chosen.unwrap_or_else(|| query.trim().to_owned());

    if draft.then_worktree && value.is_empty() {
        with_host(state, cx, |host| {
            host.card_picker.error = Some("Pick a repository".into())
        });
        notify(state, cx);
        return;
    }
    // The picker returns to the card detail when it came from there, and that dialog's scrim
    // covers the status bar: the refusal has to follow the surface the user is left looking at.
    let refusal = board::Refusal::for_detail(draft.then_detail, &card_id);

    // `Blocks` is the one kind that does not patch the card it was opened on: the link lives on
    // each dependant, so the set turns into one `UpdateCard` per card whose membership changed
    // (contracts §5.3).
    if matches!(draft.kind, PickerKind::Blocks) {
        let updates = blocks_updates(state.read(cx), &card, &draft.selected);
        send_each(state, bridge, updates, refusal, cx);
        close(state, cx);
        return;
    }

    let Some(request) = request_for(&draft, &card_id, &value, schema, card.agent.as_ref()) else {
        with_host(state, cx, |host| {
            host.card_picker.error = Some(format!("`{value}` is not a valid value"));
        });
        notify(state, cx);
        return;
    };

    if draft.then_worktree {
        // §8's `w` asked for a repository first: one request links the repo and creates the
        // worktree, so the daemon can never see the create before the repo it needs.
        board::request_worktree_reporting(
            card_id,
            RepoId::try_from(value.as_str()).ok(),
            state,
            bridge,
            refusal,
            cx,
        );
    } else {
        board::send_card_reporting(state, bridge, request, refusal, cx);
    }
    close(state, cx);
}

/// Closes the picker, returning to the dialog it was opened over when there was one.
///
/// `then_detail` is what makes `Enter` on a property row of the card detail land back on that
/// card rather than on the board, so the picker reads as a step inside the detail (BOARD §8).
pub(super) fn close(state: &Entity<AppState>, cx: &mut App) {
    let then_detail = with_host(state, cx, |host| {
        let dialog = host.card_picker.return_dialog();
        if dialog.is_some() {
            host.open = dialog.clone();
        }
        host.card_picker_input = None;
        host.card_picker_input_subscription = None;
        dialog
    });
    state.update(cx, |app, cx| {
        if let Some(dialog) = then_detail {
            app.open_overlay(crate::state::Overlay::Dialog(dialog));
        } else {
            app.close_overlay();
        }
        cx.notify();
    });
}

/// The `UpdateCard`s that make the card's dependants match the `Blocks` picker's set.
///
/// Only the cards whose membership actually changed are in the list: re-sending the blockers
/// of every other card on the board would touch documents the user never edited, and each one
/// is an activity entry somebody has to read. The order is the cards' own key order, so a
/// refusal partway through stops at a card the user can name.
#[must_use]
pub(super) fn blocks_updates(
    state: &AppState,
    card: &fleet_core::board::Card,
    selected: &[String],
) -> Vec<(CardId, Vec<CardId>)> {
    let Some(view) = state.board() else {
        return Vec::new();
    };
    let mut updates: Vec<(String, CardId, Vec<CardId>)> = Vec::new();
    for other in view.cards.iter().filter(|other| other.id != card.id) {
        let blocked = other.blocked_by.contains(&card.id);
        let wanted = selected.iter().any(|value| value == other.id.as_str());
        if blocked == wanted {
            continue;
        }
        let mut links = other.blocked_by.clone();
        if wanted {
            links.push(card.id.clone());
        } else {
            links.retain(|blocker| *blocker != card.id);
        }
        updates.push((other.display_key(&view.board), other.id.clone(), links));
    }
    updates.sort_by(|left, right| left.0.cmp(&right.0));
    updates
        .into_iter()
        .map(|(_, card_id, links)| (card_id, links))
        .collect()
}

/// Sends one `UpdateCard` per entry, in order, stopping at the first refusal.
///
/// Sequential rather than fired all at once because the contract is "stop at the first
/// refusal": a batch would leave the user with one sentence and no way to tell which of the
/// other cards it did or did not reach. Everything patched before the refusal stays patched —
/// the daemon accepted those — and the sentence is the daemon's own, on the surface the picker
/// returned to.
pub(super) fn send_each(
    state: &Entity<AppState>,
    bridge: &Bridge,
    updates: Vec<(CardId, Vec<CardId>)>,
    refusal: board::Refusal,
    cx: &mut App,
) {
    if updates.is_empty() {
        return;
    }
    let state = state.clone();
    let bridge = bridge.clone();
    cx.spawn(async move |cx| {
        for (card_id, blocked_by) in updates {
            let reply = bridge.request(RequestBody::UpdateCard {
                card_id,
                patch: CardPatch {
                    blocked_by: Some(blocked_by),
                    ..CardPatch::default()
                },
            });
            let answer = match reply.recv().await {
                Ok(answer) => answer,
                Err(error) => {
                    let message = format!("Board request channel closed: {error}");
                    cx.update(|cx| refusal.clone().report(&state, message, cx));
                    return;
                }
            };
            match answer {
                Ok(ResponseBody::Card(card)) => cx.update(|cx| {
                    state.update(cx, |app, cx| {
                        app.apply_card(card);
                        cx.notify();
                    });
                }),
                Ok(_) => cx.update(|cx| {
                    state.update(cx, |app, cx| {
                        app.board_stale = true;
                        cx.notify();
                    });
                }),
                Err(error) => {
                    cx.update(|cx| refusal.clone().report(&state, error.message, cx));
                    return;
                }
            }
        }
    })
    .detach();
}

/// The request one applied pick turns into.
pub(super) fn request_for(
    draft: &CardPickerState,
    card_id: &CardId,
    value: &str,
    schema: Option<PropertyKind>,
    agent: Option<&CardAgentPrefs>,
) -> Option<RequestBody> {
    let empty = value.is_empty();
    let patch = |patch: CardPatch| {
        Some(RequestBody::UpdateCard {
            card_id: card_id.clone(),
            patch,
        })
    };
    match &draft.kind {
        PickerKind::Status => Some(RequestBody::MoveCard {
            card_id: card_id.clone(),
            status_id: StatusId::try_from(value).ok()?,
            index: None,
            cancel_run: false,
        }),
        PickerKind::Priority => patch(CardPatch {
            priority: Some(parse_priority(value)?),
            ..CardPatch::default()
        }),
        PickerKind::Assignee => patch(CardPatch {
            assignee: Some((!empty).then(|| value.to_owned())),
            ..CardPatch::default()
        }),
        PickerKind::Labels => patch(CardPatch {
            labels: Some(
                draft
                    .selected
                    .iter()
                    .filter_map(|id| LabelId::try_from(id.as_str()).ok())
                    .collect(),
            ),
            ..CardPatch::default()
        }),
        PickerKind::Estimate => patch(CardPatch {
            estimate: Some(if empty {
                None
            } else {
                Some(value.parse::<u32>().ok()?)
            }),
            ..CardPatch::default()
        }),
        PickerKind::DueDate => patch(CardPatch {
            due_date: Some(if empty {
                None
            } else {
                Some(is_iso_date(value).then(|| value.to_owned())?)
            }),
            ..CardPatch::default()
        }),
        PickerKind::Repo => patch(CardPatch {
            repo_id: Some(if empty {
                None
            } else {
                Some(RepoId::try_from(value).ok()?)
            }),
            ..CardPatch::default()
        }),
        PickerKind::Property(key) => {
            let value = property_value(schema?, &draft.selected, value)?;
            patch(CardPatch {
                properties: Some([(key.clone(), value)].into_iter().collect()),
                ..CardPatch::default()
            })
        }
        PickerKind::BlockedBy => patch(CardPatch {
            // The whole set, the way labels send the whole set: a blocker the daemon no longer
            // knows drops out here rather than travelling as an id nothing resolves.
            blocked_by: Some(
                draft
                    .selected
                    .iter()
                    .filter_map(|id| CardId::try_from(id.as_str()).ok())
                    .collect(),
            ),
            ..CardPatch::default()
        }),
        // `Blocks` writes the dependants, not this card: `apply` sends that sequence itself and
        // never reaches here.
        PickerKind::Blocks => None,
        PickerKind::Provider | PickerKind::Model | PickerKind::Effort => {
            // One field of the card's whole `agent` block is replaced and the block is sent
            // whole, so the last writer wins exactly as it does for labels (contracts §5.3).
            let mut prefs = agent.cloned().unwrap_or_default();
            match &draft.kind {
                PickerKind::Provider => {
                    prefs.provider = if empty {
                        None
                    } else {
                        Some(parse_provider(value)?)
                    };
                }
                PickerKind::Model => prefs.model = (!empty).then(|| value.to_owned()),
                _ => prefs.effort = (!empty).then(|| value.to_owned()),
            }
            patch(CardPatch {
                // An empty block is no block at all: once the last of the three is back on the
                // column's default, `Some(None)` takes the override off the card entirely.
                agent: Some((!prefs.is_empty()).then_some(prefs)),
                ..CardPatch::default()
            })
        }
    }
}

/// The typed value a custom property takes, or `None` when the text does not fit its kind.
pub(super) fn property_value(
    kind: PropertyKind,
    selected: &[String],
    value: &str,
) -> Option<PropertyValue> {
    if value.is_empty() && kind != PropertyKind::MultiSelect {
        return Some(PropertyValue::Null);
    }
    Some(match kind {
        PropertyKind::Text => PropertyValue::Text(value.to_owned()),
        PropertyKind::Number => PropertyValue::Number(value.parse().ok()?),
        PropertyKind::Bool => PropertyValue::Bool(value == "true"),
        PropertyKind::Date => PropertyValue::Date(is_iso_date(value).then(|| value.to_owned())?),
        PropertyKind::Select => PropertyValue::Select(value.to_owned()),
        PropertyKind::MultiSelect => PropertyValue::MultiSelect(selected.to_vec()),
        PropertyKind::User => PropertyValue::User(value.to_owned()),
        PropertyKind::Url => PropertyValue::Url(value.to_owned()),
    })
}

/// The provider a picker row's value names, spelled the way the row reads it.
///
/// `AgentKind` has no parser of its own: the two harnesses are named by their executable
/// everywhere a person types one, and the picker offers no other word.
#[must_use]
pub(super) fn parse_provider(value: &str) -> Option<AgentKind> {
    [AgentKind::Claude, AgentKind::Codex]
        .into_iter()
        .find(|provider| provider.executable() == value)
}

/// The priority a picker row's value names.
#[must_use]
pub(super) fn parse_priority(value: &str) -> Option<Priority> {
    Priority::ALL
        .into_iter()
        .find(|priority| format!("{priority:?}").to_lowercase() == value)
}
