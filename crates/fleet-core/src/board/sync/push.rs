use super::*;

pub(super) fn differing_fields(
    board: &Board,
    card: &Card,
    remote: &RemoteCard,
    parent: Option<Option<CardId>>,
) -> Vec<String> {
    let mut schema = board.clone();
    let mut projected = card.clone();
    apply_remote(&mut schema, &mut projected, remote, &card.updated_at);
    if let Some(parent) = parent {
        projected.parent_id = parent;
    }
    let mut fields = Vec::new();
    macro_rules! compare {
        ($($field:ident),+ $(,)?) => { $(
            if card.$field != projected.$field { fields.push(stringify!($field).into()); }
        )+ };
    }
    compare!(
        title,
        description,
        status_id,
        priority,
        assignee,
        estimate,
        due_date,
        parent_id,
        properties
    );
    // Labels represent a set, not an order, and remote ids may differ from local names.
    if card.labels.iter().collect::<BTreeSet<_>>()
        != projected.labels.iter().collect::<BTreeSet<_>>()
    {
        fields.push("labels".into());
    }
    // A field the backend cannot write back is never a side the user can hold. `ops` refuses a
    // local edit to one, so a difference here is always the remote's alone: listing it makes the
    // conflict banner offer a choice, and `KeepLocal` then plans an `Update` the backend drops
    // on the floor while acknowledging it as pushed — the card records "Pushed local changes",
    // clears `dirty`, and the next pull puts the remote's value back. The remote simply owns it.
    fields.retain(|field| {
        !board
            .sync
            .readonly_fields
            .iter()
            .any(|readonly| readonly == field)
    });
    fields
}

pub(super) fn plan_push(
    board: &Board,
    card: &Card,
    remote: Option<&RemoteCard>,
    parent: Option<Option<CardId>>,
    caps: BackendCapabilities,
    ops: &mut Vec<PushOp>,
) {
    if board.backend.is_local() {
        return;
    }
    if card.remote.is_none() {
        if board.settings.push_new_cards && caps.push_create {
            ops.push(PushOp::Create {
                card_id: card.id.clone(),
            });
            // A create carries no status: the remote files the issue in whatever its workflow
            // opens with, so a card born in any other column lands in the wrong one — and the
            // ack stamps the create's own version, so no later pull, incremental or full, ever
            // notices. The move has to be pushed like any other, right behind the create that
            // gave it something to move. A backend already sitting on that status answers the
            // transition as the no-op it is.
            if caps.transitions
                && let Some(remote_status) =
                    board.sync.status_map.local_to_remote.get(&card.status_id)
            {
                ops.push(PushOp::Transition {
                    card_id: card.id.clone(),
                    remote_status: remote_status.clone(),
                });
            }
        } else {
            return;
        }
    } else if card.dirty && remote.is_none() {
        // An incremental omission is not a baseline. The service fetches one before pushing.
        return;
    } else if card.dirty {
        let Some(remote) = remote else {
            return;
        };
        let mut fields = differing_fields(board, card, remote, parent);
        let transition = fields.iter().any(|field| field == "status_id");
        fields.retain(|field| {
            field != "status_id" && (caps.custom_properties || field != "properties")
        });
        if caps.push_updates && !fields.is_empty() {
            ops.push(PushOp::Update {
                card_id: card.id.clone(),
                fields,
            });
        }
        if transition
            && caps.transitions
            && let Some(remote_status) = board.sync.status_map.local_to_remote.get(&card.status_id)
        {
            ops.push(PushOp::Transition {
                card_id: card.id.clone(),
                remote_status: remote_status.clone(),
            });
        }
    }
    if caps.comments {
        ops.extend(
            card.comments
                .iter()
                .filter(|comment| comment.remote_id.is_none())
                .map(|comment| PushOp::AddComment {
                    card_id: card.id.clone(),
                    comment_id: comment.id.clone(),
                }),
        );
    }
}
