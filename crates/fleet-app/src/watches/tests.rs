use super::*;
use fleet_core::ids::TerminalId;
use std::time::Duration;
pub(super) fn watch(id: u64) -> Watch {
    Watch {
        id: WatchId(id),
        session: "repo/main".parse().unwrap(),
        terminal: TerminalId(1),
        label: format!("child {id}"),
        command: vec!["sh".into()],
        cwd: None,
        pid: None,
        started_at: chrono::Utc::now().to_rfc3339(),
        status: WatchStatus::Running,
        source: fleet_core::watches::WatchSource::Cooperative,
        log_file: None,
    }
}
pub(super) fn chunk(seq: u64, stream: WatchStream, text: &str) -> WatchChunk {
    WatchChunk {
        seq,
        stream,
        text: text.into(),
    }
}
#[test]
fn cycling_wraps_in_start_order_within_the_session_and_reopens_hidden_panes() {
    let mut state = Watches::default();
    let now = Instant::now();
    let session = watch(1).session;
    assert!(!state.cycle(&session, true));
    assert!(!state.cycle(&session, false));
    state.started(watch(1), now);
    for forward in [true, false] {
        state.hide(&session);
        assert!(state.cycle(&session, forward));
        assert!(state.panes[&session].visible);
        assert_eq!(state.panes[&session].selected, Some(WatchId(1)));
    }
    let mut other = watch(2);
    other.session = "repo/other".parse().unwrap();
    state.started(other.clone(), now);
    // Events can arrive out of order; monotonic IDs preserve start order.
    state.started(watch(4), now);
    state.started(watch(3), now);
    state.select(&session, WatchId(1));
    for (forward, expected) in [(false, 4), (true, 1), (true, 3), (true, 4), (false, 3)] {
        state.hide(&session);
        assert!(state.cycle(&session, forward));
        assert_eq!(state.panes[&session].selected, Some(WatchId(expected)));
        assert!(state.panes[&session].visible);
        assert_eq!(state.panes[&other.session].selected, Some(other.id));
    }
}

#[test]
fn a_new_start_still_opens_when_its_list_response_arrived_first() {
    let mut state = Watches::default();
    let now = Instant::now();
    let w = watch(1);
    state.listed(&w.session, vec![], vec![w.clone()], now);
    state.hide(&w.session);
    state.started(w.clone(), now);
    assert!(state.panes[&w.session].visible);
    state.hide(&w.session);
    state.started(w.clone(), now);
    assert!(!state.panes[&w.session].visible);
}

#[test]
fn lines_are_assembled_per_stream_with_live_partials_and_no_duplicate_replay() {
    let mut state = Watches::default();
    let now = Instant::now();
    let w = watch(1);
    state.started(w.clone(), now);
    state.output(
        w.id,
        vec![
            chunk(0, WatchStream::Stdout, "hel"),
            chunk(1, WatchStream::Stderr, "warn"),
            chunk(2, WatchStream::Stdout, "lo\nsecond\n"),
            chunk(3, WatchStream::Stderr, "ing\npartial"),
        ],
    );
    let (lines, tones) = state.entries[&w.id].shared_display();
    assert_eq!(
        lines
            .iter()
            .map(gpui::SharedString::as_ref)
            .collect::<Vec<_>>(),
        ["hello", "second", "warning", "partial"]
    );
    assert_eq!(
        tones.as_ref(),
        [
            fleet_ui_kit::Tone::Default,
            fleet_ui_kit::Tone::Default,
            fleet_ui_kit::Tone::Secondary,
            fleet_ui_kit::Tone::Secondary
        ]
    );
    state.output(w.id, vec![chunk(2, WatchStream::Stdout, "duplicated\n")]);
    assert_eq!(state.entries[&w.id].next_seq, 4);
    assert_eq!(state.entries[&w.id].lines.len(), 3);
    assert!(state.take_tails().is_empty());
}
#[test]
fn gap_requests_one_tail_and_merges_overlapping_live_output() {
    let mut state = Watches::default();
    let now = Instant::now();
    let w = watch(1);
    state.started(w.clone(), now);
    state.output(
        w.id,
        vec![
            chunk(0, WatchStream::Stdout, "one\n"),
            chunk(2, WatchStream::Stdout, "three\n"),
        ],
    );
    assert_eq!(state.take_tails(), vec![(w.id, Some(1))]);
    assert!(state.take_tails().is_empty());
    state.output(w.id, vec![chunk(3, WatchStream::Stdout, "four\n")]);
    state.tailed(
        WatchTail {
            watch: w.clone(),
            chunks: vec![
                chunk(0, WatchStream::Stdout, "one\n"),
                chunk(1, WatchStream::Stdout, "two\n"),
                chunk(2, WatchStream::Stdout, "three\n"),
            ],
            first_retained_seq: 0,
            next_seq: 3,
        },
        now,
    );
    assert_eq!(state.entries[&w.id].lines.len(), 3);
    assert_eq!(state.take_tails(), vec![(w.id, Some(3))]);
    state.tailed(
        WatchTail {
            watch: w.clone(),
            chunks: vec![chunk(3, WatchStream::Stdout, "four\n")],
            first_retained_seq: 0,
            next_seq: 4,
        },
        now,
    );
    assert_eq!(state.entries[&w.id].next_seq, 4);
    assert!(state.take_tails().is_empty());
}
#[test]
fn retention_gap_does_not_join_partial_text_across_lost_output() {
    let mut state = Watches::default();
    let now = Instant::now();
    let w = watch(1);
    state.started(w.clone(), now);
    state.output(w.id, vec![chunk(0, WatchStream::Stdout, "partial")]);
    state.tailed(
        WatchTail {
            watch: w.clone(),
            chunks: vec![chunk(8, WatchStream::Stdout, "retained\n")],
            first_retained_seq: 8,
            next_seq: 9,
        },
        now,
    );
    let mirror = &state.entries[&w.id];
    assert!(mirror.trimmed);
    assert_eq!(mirror.next_seq, 9);
    assert_eq!(mirror.lines[0].text, "retained");
}
#[test]
fn hiding_survives_duplicates_output_exit_and_list_until_a_new_start() {
    let now = Instant::now();
    let mut state = Watches::default();
    let w = watch(1);
    let session = w.session.clone();
    assert!(!state.toggle(&session));
    state.started(w.clone(), now);
    assert!(state.panes[&session].visible);
    assert!(state.toggle(&session));
    state.started(w.clone(), now);
    state.output(w.id, vec![chunk(0, WatchStream::Stdout, "x\n")]);
    let mut exited = w.clone();
    exited.status = WatchStatus::Exited {
        code: Some(3),
        signal: None,
    };
    state.exited(exited.clone(), now + Duration::from_secs(3));
    state.listed(&session, vec![w.id], vec![exited], now);
    assert!(!state.panes[&session].visible);
    let elapsed = state.entries[&w.id].elapsed(now + Duration::from_secs(10));
    assert!(elapsed >= Duration::from_secs(3) && elapsed < Duration::from_secs(4));
    state.started(w, now);
    assert!(matches!(
        state.entries[&WatchId(1)].watch.status,
        WatchStatus::Exited { .. }
    ));
    state.started(watch(2), now);
    assert!(state.panes[&session].visible);
    assert_eq!(state.panes[&session].selected, Some(WatchId(2)));
    state.toggle(&session);
    state.toggle(&session);
    assert_eq!(state.panes[&session].selected, Some(WatchId(2)));
}
#[test]
fn dismissal_selects_next_closes_last_and_stale_replies_cannot_resurrect() {
    let now = Instant::now();
    let mut state = Watches::default();
    let w = watch(1);
    let session = w.session.clone();
    state.started(w.clone(), now);
    state.started(watch(2), now);
    state.dismissed(WatchId(2));
    assert_eq!(state.panes[&session].selected, Some(w.id));
    state.dismissed(w.id);
    assert!(!state.panes[&session].visible);
    state.tailed(
        WatchTail {
            watch: w.clone(),
            chunks: vec![],
            first_retained_seq: 0,
            next_seq: 0,
        },
        now,
    );
    state.listed(&session, vec![], vec![w], now);
    assert!(state.entries.is_empty());
    assert_eq!(state.panes[&session].selected, None);
}
#[test]
fn list_reconciles_known_ids_but_preserves_concurrent_starts_and_local_hiding() {
    let now = Instant::now();
    let mut state = Watches::default();
    let session = watch(1).session;
    state.listed(&session, vec![], vec![watch(1), watch(2)], now);
    assert_eq!(state.panes[&session].selected, Some(WatchId(2)));
    assert_eq!(
        state.take_tails(),
        vec![(WatchId(1), None), (WatchId(2), None)]
    );
    state.hide(&session);
    state.started(watch(3), now);
    state.hide(&session);
    state.listed(&session, vec![WatchId(1), WatchId(2)], vec![watch(2)], now);
    assert!(!state.entries.contains_key(&WatchId(1)));
    assert!(state.entries.contains_key(&WatchId(3)));
    assert_eq!(state.panes[&session].selected, Some(WatchId(3)));
    assert!(!state.panes[&session].visible);
}
#[test]
fn session_entry_reconnect_and_lag_request_fresh_lists() {
    let mut state = Watches::default();
    let session = watch(1).session;
    assert_eq!(state.enter(Some(session.clone()), 1), Some(session.clone()));
    assert_eq!(state.enter(Some(session.clone()), 1), None);
    assert_eq!(state.enter(None, 1), None);
    assert_eq!(state.enter(Some(session.clone()), 1), Some(session.clone()));
    assert_eq!(state.enter(Some(session.clone()), 2), Some(session.clone()));
    state.invalidate();
    assert_eq!(state.enter(Some(session.clone()), 2), Some(session));
}

#[test]
fn display_snapshots_share_text_and_ignore_duplicate_chunks() {
    let now = Instant::now();
    let mut state = Watches::default();
    let watch = watch(1);
    state.started(watch.clone(), now);
    let text = "a long line whose text must be shared rather than copied";
    state.output(
        watch.id,
        vec![chunk(0, WatchStream::Stdout, &format!("{text}\npartial"))],
    );
    let mirror = &state.entries[&watch.id];
    let (lines, tones) = mirror.shared_display();
    let (same_lines, same_tones) = mirror.shared_display();
    assert!(std::sync::Arc::ptr_eq(&lines, &same_lines));
    assert!(std::sync::Arc::ptr_eq(&tones, &same_tones));
    assert_eq!(lines[0].as_ptr(), mirror.lines[0].text.as_ptr());
    state.output(watch.id, vec![chunk(0, WatchStream::Stdout, "duplicate")]);
    assert!(std::sync::Arc::ptr_eq(
        &lines,
        &state.entries[&watch.id].shared_display().0
    ));
    state.output(watch.id, vec![chunk(1, WatchStream::Stdout, " end")]);
    let updated = state.entries[&watch.id].shared_display().0;
    assert_eq!(updated[0].as_ptr(), lines[0].as_ptr());
    assert_eq!(updated[1], "partial end");
    assert_eq!(lines[1], "partial");
}

#[test]
fn bookkeeping_compaction_keeps_stale_replies_rejected() {
    let now = Instant::now();
    let mut state = Watches::default();
    for id in 1..=1_000 {
        let mut watch = watch(id);
        watch.session = format!("repo/session-{id}").parse().unwrap();
        state.started(watch, now);
        state.dismissed(WatchId(id));
    }
    assert!(state.started_events.is_empty());
    state.reconcile_sessions(&HashSet::new());
    assert!(state.panes.is_empty());
    state.reconnect();
    let old = watch(1);
    state.started(old.clone(), now);
    state.listed(&old.session, vec![], vec![old.clone()], now);
    state.tailed(
        WatchTail {
            watch: old,
            chunks: vec![],
            first_retained_seq: 0,
            next_seq: 0,
        },
        now,
    );
    assert!(state.entries.is_empty());
    assert!(state.started_events.is_empty());
}
