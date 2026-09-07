//! Snapshot and serialization timing, outside the correctness test suite.
#![cfg(feature = "ghostty")]

use fleet_core::ids::TerminalId;
use fleet_proto::terminal::ScrollCommand;
use fleet_term::{GhosttyEngine, VtEngine};

#[test]
#[ignore = "manual timing benchmark; run with cargo test -p fleet-term --bench viewport -- --ignored --nocapture"]
fn viewport_frame_timing_200x60() {
    use std::time::Instant;
    let mut engine = GhosttyEngine::new(200, 60, 1_073_741_824).unwrap();
    let output = (0..2000)
        .map(|i| format!("\x1b[3{}m{i:06} {}\x1b[0m\r\n", i % 8, "x".repeat(190)))
        .collect::<String>();
    engine.feed(output.as_bytes());
    engine.take_frame(true);
    for full in [true, false] {
        engine.scroll(ScrollCommand::Bottom);
        engine.take_frame(true);
        let mut samples = Vec::new();
        let mut snapshot_us = 0;
        let mut json_us = 0;
        for _ in 0..100 {
            engine.scroll(ScrollCommand::Lines(-1));
            let start = Instant::now();
            let mut frame = engine.take_frame(full);
            frame.terminal = TerminalId(1);
            frame.seq = 1;
            let snapshot = start.elapsed();
            let bytes = serde_json::to_vec(&frame).unwrap();
            std::hint::black_box(bytes);
            let elapsed = start.elapsed();
            snapshot_us += snapshot.as_micros();
            json_us += (elapsed - snapshot).as_micros();
            samples.push(elapsed.as_micros());
        }
        samples.sort_unstable();
        println!(
            "200x60 viewport frame full={full}: snapshot mean={} us; JSON mean={} us; total median={} us p95={} us max={} us (100 moves)",
            snapshot_us / 100,
            json_us / 100,
            samples[50],
            samples[95],
            samples[99]
        );
    }
}
