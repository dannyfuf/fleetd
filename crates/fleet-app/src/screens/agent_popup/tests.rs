use super::*;

#[test]
fn creation_input_survives_terminal_discovery_for_the_same_owner() {
    let owner = PendingOwner {
        agent: Agent::Claude,
        generation: 7,
    };
    let mut local = Local::default();
    local.queue_input(owner, PendingInput::Paste("first".to_owned()));
    local.queue_input(owner, PendingInput::Paste("second".to_owned()));

    assert_eq!(
        local.take_pending(owner),
        vec![
            PendingInput::Paste("first".to_owned()),
            PendingInput::Paste("second".to_owned())
        ]
    );
    assert!(local.pending.is_empty());
    assert_eq!(local.state.pending_owner, None);
}

#[test]
fn creation_input_is_discarded_on_agent_switch_or_generation_change() {
    let claude = PendingOwner {
        agent: Agent::Claude,
        generation: 7,
    };
    let opencode = PendingOwner {
        agent: Agent::Opencode,
        generation: 7,
    };
    let reconnected = PendingOwner {
        agent: Agent::Claude,
        generation: 8,
    };
    let mut local = Local::default();
    local.queue_input(claude, PendingInput::Paste("stale".to_owned()));
    local.queue_input(opencode, PendingInput::Paste("switched".to_owned()));
    assert!(local.take_pending(claude).is_empty());
    assert_eq!(
        local.take_pending(opencode),
        vec![PendingInput::Paste("switched".to_owned())]
    );

    local.queue_input(claude, PendingInput::Paste("old link".to_owned()));
    local.queue_input(reconnected, PendingInput::Paste("new link".to_owned()));
    assert!(local.take_pending(claude).is_empty());
    assert_eq!(
        local.take_pending(reconnected),
        vec![PendingInput::Paste("new link".to_owned())]
    );
}
