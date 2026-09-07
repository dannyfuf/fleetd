use super::*;

/// Opens the dialog on the board's selected card, with no edit in progress.
pub(crate) fn seed(state: &Entity<AppState>, cx: &mut App) {
    let card_id = crate::screens::board::selected_card(state.read(cx)).map(|card| card.id.clone());
    with_host(state, cx, |host| {
        host.card_detail = CardDetailState {
            card_id,
            revision: host.card_detail.revision.wrapping_add(1),
            ..Default::default()
        }
    });
}

/// `ctrl-s`: send the open buffer and wait for the daemon's answer.
pub(super) fn commit_edit(state: &Entity<AppState>, bridge: &Bridge, cx: &mut App) {
    cx.stop_propagation();
    // The same gate every other mutating key on this surface applies (`enter`, `w`, `K`/`R`):
    // without it a save with fleetd down answered with the bridge's own "the Fleet daemon is
    // not connected" on the dialog's error line and never flashed the offline banner — two
    // different sentences for one condition.
    if board::refuses(state, cx) {
        return;
    }
    let draft = read_host(state, cx, |host, _| host.card_detail.clone());
    if draft.saving.is_some() {
        return;
    }
    let (Some(surface), Some(card_id)) = (draft.edit, draft.card_id.clone()) else {
        return;
    };
    let text = draft.area.text().trim().to_owned();
    let request = match surface {
        CardEdit::Title if text.is_empty() => {
            with_host(state, cx, |host| {
                host.card_detail.error = Some("A card needs a title.".to_owned());
            });
            notify(state, cx);
            return;
        }
        CardEdit::Title => RequestBody::UpdateCard {
            card_id,
            patch: CardPatch {
                title: Some(text),
                ..CardPatch::default()
            },
        },
        CardEdit::Description => RequestBody::UpdateCard {
            card_id,
            patch: CardPatch {
                description: Some(draft.area.text().to_owned()),
                ..CardPatch::default()
            },
        },
        CardEdit::Comment if text.is_empty() => {
            with_host(state, cx, |host| host.card_detail.cancel());
            notify(state, cx);
            return;
        }
        CardEdit::Comment => RequestBody::AddCardComment {
            card_id,
            body: text,
        },
    };
    let revision = draft.revision;
    with_host(state, cx, |host| host.card_detail.saving = Some(revision));
    let reply = bridge.request(request);
    complete_request(state, cx, async move |state, cx| {
        let result = match reply.recv().await {
            Ok(Ok(ResponseBody::Card(card))) => Ok(card),
            Ok(Ok(_)) => Err("Unexpected card save response".to_owned()),
            Ok(Err(error)) => Err(error.message),
            Err(error) => Err(format!("Card save channel closed: {error}")),
        };
        cx.update(|cx| {
            let Some(state) = state.upgrade() else { return };
            with_host(&state, cx, |host| {
                host.card_detail
                    .finish_save(revision, result.as_ref().err().cloned())
            });
            if let Ok(card) = result {
                state.update(cx, |app, cx| {
                    app.apply_card(card);
                    cx.notify();
                });
            }
            notify(&state, cx);
        });
    });
}

/// What `K`/`R` answer on a card that has no conflict to resolve.
const NO_CONFLICT: &str = "No conflict on this card";

/// `K` / `R`: resolve the card's conflict one way or the other.
pub(super) fn resolve(
    state: &Entity<AppState>,
    bridge: &Bridge,
    resolution: ConflictResolution,
    cx: &mut App,
) {
    // Every other mutating key on this surface answers "fleetd is not reachable" rather than
    // sending into a dead channel and reporting the channel's own closure.
    if board::refuses(state, cx) {
        return;
    }
    let draft = read_host(state, cx, |host, _| host.card_detail.clone());
    let Some(card) = card(state.read(cx), &draft) else {
        return;
    };
    if card.conflict.is_none() {
        // §8 gives every board key an answer; a key that neither acts nor says anything reads
        // as broken.
        with_host(state, cx, |host| {
            host.card_detail.error = Some(NO_CONFLICT.to_owned());
        });
        notify(state, cx);
        return;
    }
    let card_id = card.id.clone();
    // The dialog's scrim covers the status bar: a refusal must land on this surface (§Board).
    board::send_card_reporting(
        state,
        bridge,
        RequestBody::ResolveCardConflict {
            card_id: card_id.clone(),
            resolution,
        },
        board::Refusal::CardDetail(card_id),
        cx,
    );
    cx.stop_propagation();
}
