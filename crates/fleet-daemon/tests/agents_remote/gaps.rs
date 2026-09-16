//! Gap handling for peers that cannot serve windowed mirror refills.

use super::*;

#[tokio::test]
async fn a_legacy_peer_gap_keeps_proxying_the_suffix_and_later_events() {
    let host = host("legacy-box");
    let thread = ThreadId::new();
    let machines = Arc::new(Machines::from_config(&default_config(
        "/tmp/fleet-agent-legacy-gap",
    )));
    let remote = Arc::new(FakeRemote::new(host.clone()));
    remote.set_hello(RemoteHello {
        version: "fleetd legacy".to_owned(),
        daemon_id: "owner".to_owned(),
        build_commit: None,
        capabilities: Vec::new(),
    });
    machines.install_endpoint(host.clone(), remote.clone());
    let router = Router::new(machines, Arc::new(Mirror::new()));
    let mirror = Arc::new(BatchMirror::default());
    *lock_test(&mirror.gap_after) = Some(1);
    router.set_agent_mirror(mirror.clone());
    let events = BroadcastBus::default();
    let mut receiver = events.subscribe();
    router.start_event_pumps(events);

    for seq in [1, 3, 4] {
        remote.emit(remote_agent_event(thread, seq));
    }
    let mut published = receive_agent_events(&mut receiver, 3)
        .await
        .expect("the legacy suffix stays publishable");

    *lock_test(&mirror.gap_after) = Some(0);
    remote.emit(remote_agent_event(thread, 5));
    published.extend(
        receive_agent_events(&mut receiver, 1)
            .await
            .expect("later legacy events stay publishable"),
    );

    let sequences = published
        .into_iter()
        .filter_map(|event| match event {
            Event::Agent { event, .. } => Some(event.seq),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(sequences, [Seq(1), Seq(3), Seq(4), Seq(5)]);
    assert!(
        remote.requests().is_empty(),
        "a legacy peer cannot be asked for an agent.window refill"
    );
}
