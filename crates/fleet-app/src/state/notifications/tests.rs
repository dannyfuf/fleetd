use super::*;
use crate::state::test_support::*;
use std::sync::atomic::Ordering;

#[test]
fn toasts_coalesce_within_a_second_and_cap_at_three() {
    let now = Instant::now();
    let mut toasts = Vec::new();
    let dwell = Duration::from_millis(3_200);
    push_toast(&mut toasts, Toast::new("Path copied"), now, dwell);
    push_toast(
        &mut toasts,
        Toast::new("Path copied"),
        now + Duration::from_millis(300),
        dwell,
    );
    assert_eq!(toasts.len(), 1);
    assert_eq!(toasts[0].toast.count, 2);

    push_toast(
        &mut toasts,
        Toast::new("Path copied"),
        now + Duration::from_secs(5),
        dwell,
    );
    assert_eq!(toasts.len(), 2, "past the window it is a new toast");

    for index in 0..3 {
        push_toast(
            &mut toasts,
            Toast::new(format!("toast {index}")),
            now + Duration::from_secs(6),
            dwell,
        );
    }
    assert_eq!(toasts.len(), MAX_TOASTS);
    assert_eq!(toasts[0].toast.text.as_ref(), "toast 0");
}

#[test]
fn toasts_are_never_errors() {
    let now = Instant::now();
    let mut toasts = Vec::new();
    let mut toast = Toast::new("boom");
    toast.tone = Tone::Danger;
    push_toast(&mut toasts, toast, now, Duration::from_secs(1));
    assert_eq!(toasts[0].toast.tone, Tone::Warning);
}

#[test]
fn toasts_expire() {
    let now = Instant::now();
    let mut toasts = Vec::new();
    push_toast(
        &mut toasts,
        Toast::new("gone"),
        now,
        Duration::from_millis(100),
    );
    assert!(!expire_toasts(&mut toasts, now));
    assert!(expire_toasts(&mut toasts, now + Duration::from_millis(200)));
    assert!(toasts.is_empty());
}

#[test]
fn working_to_idle_after_two_seconds_notifies_once() {
    let now = Instant::now();
    let (mut state, plays) = state_with_recording_sound(now);
    state.apply_bridge_event(
        BridgeEvent::Connected(Box::new(agent_snapshot(&[(
            "payroll/feat",
            "feat",
            1,
            AgentActivity::Unknown,
        )]))),
        now,
    );

    state.apply_daemon_event(agent_event("payroll/feat", 1, AgentActivity::Working), now);
    state.apply_daemon_event(
        agent_event("payroll/feat", 1, AgentActivity::Idle),
        now + Duration::from_secs(2),
    );
    state.apply_daemon_event(
        agent_event("payroll/feat", 1, AgentActivity::Idle),
        now + Duration::from_secs(3),
    );

    assert_eq!(state.toasts.len(), 1);
    assert_eq!(
        state.toasts[0].toast.text.as_ref(),
        "payroll/feat: agent finished"
    );
    assert_eq!(state.toasts[0].toast.icon, Some(Icon::CircleCheck));
    assert_eq!(state.toasts[0].toast.tone, Tone::Success);
    assert_eq!(plays.load(Ordering::SeqCst), 1);
}

#[test]
fn popup_agent_activity_survives_snapshots_and_notifies_once() {
    let now = Instant::now();
    let (mut state, plays) = state_with_recording_sound(now);
    let session = fleet_core::sessions::agent_session_id(Agent::Claude)
        .unwrap_or_else(|error| panic!("{error}"));
    let mut fixed_agent = session_with(session.as_str(), &[41]);
    fixed_agent.kind = SessionKind::Agent(Agent::Claude);
    let mut current = snapshot();
    current.sessions.push(fixed_agent);
    state.apply_bridge_event(BridgeEvent::Connected(Box::new(current.clone())), now);
    state.toggle_agent_popup(Agent::Claude, None);

    state.apply_daemon_event(
        agent_event(session.as_str(), 41, AgentActivity::Working),
        now,
    );
    assert_eq!(
        state.session_agent_activity(&session),
        AgentActivity::Working
    );

    state.apply_snapshot(current, now + Duration::from_secs(1));
    assert_eq!(
        state.session_agent_activity(&session),
        AgentActivity::Working,
        "an unrelated snapshot must not erase fixed-agent activity"
    );

    state.apply_daemon_event(
        agent_event(session.as_str(), 41, AgentActivity::Idle),
        now + Duration::from_secs(2),
    );
    assert_eq!(state.toasts.len(), 1);
    assert_eq!(plays.load(Ordering::SeqCst), 1);
}

#[test]
fn short_working_flicker_does_not_notify() {
    let now = Instant::now();
    let (mut state, plays) = state_with_recording_sound(now);
    state.apply_daemon_event(agent_event("payroll/feat", 1, AgentActivity::Working), now);
    state.apply_daemon_event(
        agent_event("payroll/feat", 1, AgentActivity::Idle),
        now + Duration::from_millis(1_999),
    );
    assert!(state.toasts.is_empty());
    assert_eq!(plays.load(Ordering::SeqCst), 0);
}

#[test]
fn unknown_to_idle_does_not_notify() {
    let now = Instant::now();
    let (mut state, plays) = state_with_recording_sound(now);
    state.apply_daemon_event(
        agent_event("payroll/feat", 1, AgentActivity::Idle),
        now + Duration::from_secs(3),
    );
    assert!(state.toasts.is_empty());
    assert_eq!(plays.load(Ordering::SeqCst), 0);
}

#[test]
fn reconnect_with_idle_sessions_seeds_silently() {
    let now = Instant::now();
    let (mut state, plays) = state_with_recording_sound(now);
    state.apply_daemon_event(agent_event("payroll/feat", 1, AgentActivity::Working), now);
    state.apply_bridge_event(
        BridgeEvent::Reconnected {
            restarted: false,
            snapshot: Box::new(agent_snapshot(&[(
                "payroll/feat",
                "feat",
                1,
                AgentActivity::Idle,
            )])),
        },
        now + Duration::from_secs(3),
    );
    assert!(state.toasts.is_empty());
    assert_eq!(plays.load(Ordering::SeqCst), 0);
}

#[test]
fn two_sessions_finishing_produce_two_notifications() {
    let now = Instant::now();
    let (mut state, plays) = state_with_recording_sound(now);
    state.apply_daemon_event(agent_event("payroll/one", 1, AgentActivity::Working), now);
    state.apply_daemon_event(agent_event("payroll/two", 2, AgentActivity::Working), now);
    state.apply_daemon_event(
        agent_event("payroll/one", 1, AgentActivity::Idle),
        now + Duration::from_secs(2),
    );
    state.apply_daemon_event(
        agent_event("payroll/two", 2, AgentActivity::Idle),
        now + Duration::from_secs(2),
    );
    assert_eq!(state.toasts.len(), 2);
    assert_eq!(plays.load(Ordering::SeqCst), 2);
}

#[test]
fn sound_can_be_disabled_without_disabling_the_toast() {
    let now = Instant::now();
    let (mut state, plays) = state_with_recording_sound(now);
    state.notifications.sound = false;
    state.apply_daemon_event(agent_event("payroll/feat", 1, AgentActivity::Working), now);
    state.apply_daemon_event(
        agent_event("payroll/feat", 1, AgentActivity::Idle),
        now + Duration::from_secs(2),
    );
    assert_eq!(state.toasts.len(), 1);
    assert_eq!(plays.load(Ordering::SeqCst), 0);
}

#[test]
fn a_full_snapshot_recovers_a_missed_finish_event() {
    let now = Instant::now();
    let (mut state, plays) = state_with_recording_sound(now);
    state.apply_bridge_event(
        BridgeEvent::Connected(Box::new(agent_snapshot(&[(
            "payroll/feat",
            "feat",
            1,
            AgentActivity::Working,
        )]))),
        now,
    );
    state.apply_snapshot(
        agent_snapshot(&[("payroll/feat", "feat", 1, AgentActivity::Idle)]),
        now + Duration::from_secs(2),
    );
    assert_eq!(state.toasts.len(), 1);
    assert_eq!(plays.load(Ordering::SeqCst), 1);
}

#[test]
fn the_newest_failure_owns_the_sticky_slot() {
    let jobs = vec![
        job(
            "j-1",
            JobStatus::Failed {
                error: "old".to_owned(),
            },
            Some("2026-09-04T12:00:00Z"),
        ),
        job("j-2", JobStatus::Running, None),
        job(
            "j-3",
            JobStatus::Failed {
                error: "new".to_owned(),
            },
            Some("2026-09-04T12:05:00Z"),
        ),
    ];
    let failed = latest_failed_job(&jobs).unwrap_or_else(|| panic!("expected a failure"));
    assert_eq!(failed.id.as_str(), "j-3");
    assert_eq!(running_jobs(&jobs).len(), 1);
}
