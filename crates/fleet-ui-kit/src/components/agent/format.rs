//! The summary, fold and footer strings of the agent transcript.
//!
//! The canvas fixes this copy word for word (`NATIVE-AGENTS.md` §2), so it is formatted once
//! here instead of at every call site: the projection hands over numbers, these functions turn
//! them into the exact sentence. Keycaps are **not** part of these strings — those are drawn
//! with [`crate::components::KeyHint`] by the row that owns them.

use gpui::SharedString;

/// U+2212 MINUS SIGN: the character the canvas uses for a removed-line count, so `+36 −3`
/// lines up under a proportional face instead of drifting like a hyphen does.
pub const MINUS: char = '\u{2212}';

/// Formats a duration the way every agent summary states one: `1.2s`, `6s`, `48s`, `48m`, `2h`.
///
/// Under ten seconds the tenth is kept, because that is where a tool call's cost is legible;
/// above it a tenth is noise. A whole value never renders a trailing `.0`.
#[must_use]
pub fn format_duration(ms: u64) -> SharedString {
    if ms < 10_000 {
        let tenths = (ms + 50) / 100;
        let (whole, tenth) = (tenths / 10, tenths % 10);
        return if tenth == 0 {
            SharedString::from(format!("{whole}s"))
        } else {
            SharedString::from(format!("{whole}.{tenth}s"))
        };
    }
    let seconds = ms / 1_000;
    if seconds < 60 {
        SharedString::from(format!("{seconds}s"))
    } else if seconds < 3_600 {
        SharedString::from(format!("{}m", seconds / 60))
    } else {
        SharedString::from(format!("{}h", seconds / 3_600))
    }
}

/// Formats a token count the way the footer states one: `842`, `3.1k`, `12.4k`, `1.2M`.
#[must_use]
pub fn format_token_count(tokens: u64) -> SharedString {
    const K: u64 = 1_000;
    const M: u64 = 1_000_000;
    const G: u64 = 1_000_000_000;

    if tokens < K {
        return SharedString::from(tokens.to_string());
    }
    let (unit, suffix) = if tokens < M {
        (K, 'k')
    } else if tokens < G {
        (M, 'M')
    } else {
        (G, 'B')
    };
    let unit = u128::from(unit);
    let tenths = (u128::from(tokens) * 10 + unit / 2) / unit;
    let (whole, tenth) = (tenths / 10, tenths % 10);
    if tenth == 0 {
        SharedString::from(format!("{whole}{suffix}"))
    } else {
        SharedString::from(format!("{whole}.{tenth}{suffix}"))
    }
}

/// Formats one file's line counts as `+36 −3`.
#[must_use]
pub fn format_file_delta(added: u64, removed: u64) -> SharedString {
    SharedString::from(format!("+{added} {MINUS}{removed}"))
}

/// Formats the completed-turn file clause as `2 files +36 −3`, or nothing when the turn
/// touched no file.
#[must_use]
pub fn format_files_changed(files: usize, added: u64, removed: u64) -> Option<SharedString> {
    if files == 0 {
        return None;
    }
    let noun = if files == 1 { "file" } else { "files" };
    Some(SharedString::from(format!(
        "{files} {noun} {}",
        format_file_delta(added, removed)
    )))
}

/// Formats a turn's cost as `$0.42`, the way the footer states one.
#[must_use]
pub fn format_cost(cost_usd: f64) -> SharedString {
    SharedString::from(format!("${cost_usd:.2}"))
}

/// The settled-turn footer segments, in canvas order and **never invented**:
/// `12.4k tokens`, `$0.42`, `2 files +36 −3`.
///
/// A harness that does not report a figure contributes no segment, which is why every argument
/// is an [`Option`] and the result can be empty: Codex reports no cost, and a turn that touched
/// no file has no file clause. `[⏎] diff` and `[u] revert turn` are keycaps, so the footer row
/// draws those itself with [`crate::components::KeyHint`].
#[must_use]
pub fn turn_footer_segments(
    tokens: Option<u64>,
    cost_usd: Option<f64>,
    files: Option<(usize, u64, u64)>,
) -> Vec<SharedString> {
    let mut segments = Vec::new();
    if let Some(tokens) = tokens {
        segments.push(SharedString::from(format!(
            "{} tokens",
            format_token_count(tokens)
        )));
    }
    if let Some(cost) = cost_usd {
        segments.push(format_cost(cost));
    }
    if let Some((files, added, removed)) = files
        && let Some(clause) = format_files_changed(files, added, removed)
    {
        segments.push(clause);
    }
    segments
}

/// The settled-turn fold: `worked 22s · 14 steps`.
///
/// The duration is the **model's**, not the wall clock's: every interval a gate stood open is
/// the user's time and the reducer has already subtracted it, which is why a turn that was on
/// screen for four minutes folds as `worked 22s` (`NATIVE-AGENTS.md` §5).
#[must_use]
pub fn format_worked(duration_ms: Option<u64>, steps: usize) -> SharedString {
    let Some(duration_ms) = duration_ms else {
        // A harness that reported no duration gets the bare verb rather than an invented `0s`.
        return SharedString::new_static("worked");
    };
    let noun = if steps == 1 { "step" } else { "steps" };
    SharedString::from(format!(
        "worked {} · {steps} {noun}",
        format_duration(duration_ms)
    ))
}

/// The interrupted-turn fold: `you stopped after 12s`.
#[must_use]
pub fn format_stopped_after(duration_ms: u64) -> SharedString {
    SharedString::from(format!(
        "you stopped after {}",
        format_duration(duration_ms)
    ))
}

/// The collapsed reasoning line: `thought 12s`.
///
/// One spelling for both harnesses on purpose (`spec-B` §B3.3): Claude streams raw thinking and
/// Codex streams its own summary, and the user should not have to learn two vocabularies.
#[must_use]
pub fn format_thought(duration_ms: u64) -> SharedString {
    SharedString::from(format!("thought {}", format_duration(duration_ms)))
}

/// The `working 1m 12s` label of the live working row.
///
/// Not [`format_duration`]: this one ticks once a second, so it keeps the seconds part past a
/// minute — a label that reads `1m` for sixty seconds looks like a frozen clock. Under a minute
/// it is raw seconds.
#[must_use]
pub fn format_working(elapsed_ms: u64) -> SharedString {
    let seconds = elapsed_ms / 1_000;
    let (hours, minutes, seconds) = (seconds / 3_600, (seconds % 3_600) / 60, seconds % 60);
    let elapsed = if hours > 0 {
        format!("{hours}h {minutes}m")
    } else if minutes > 0 {
        format!("{minutes}m {seconds}s")
    } else {
        format!("{seconds}s")
    };
    SharedString::from(format!("working {elapsed}"))
}

/// A command's structured exit status: `exit 0`, `exit 1`.
///
/// §5: exit codes are structured fields rendered explicitly. Fleet never substring-matches
/// English error text to infer failure.
#[must_use]
pub fn format_exit(code: i32) -> SharedString {
    SharedString::from(format!("exit {code}"))
}

/// The head-of-queue counter a decision drawer draws when several requests are pending: `2/3`.
#[must_use]
pub fn format_counter(index: usize, total: usize) -> SharedString {
    SharedString::from(format!("{}/{total}", index + 1))
}

/// The retry line of an error card: `rate limited · retrying in 12s`.
#[must_use]
pub fn format_retrying(reason: impl AsRef<str>, retry_in_ms: u64) -> SharedString {
    SharedString::from(format!(
        "{} · retrying in {}",
        reason.as_ref(),
        format_duration(retry_in_ms)
    ))
}

/// A compaction checkpoint: `context compacted · 84k → 12k tokens`.
#[must_use]
pub fn format_compacted(before: u64, after: Option<u64>) -> SharedString {
    match after {
        Some(after) => SharedString::from(format!(
            "context compacted · {} → {} tokens",
            format_token_count(before),
            format_token_count(after)
        )),
        None => SharedString::from(format!(
            "context compacted · {} tokens",
            format_token_count(before)
        )),
    }
}

/// A resume checkpoint: `session resumed · 2h ago`.
#[must_use]
pub fn format_resumed(age_ms: u64) -> SharedString {
    SharedString::from(format!("session resumed · {} ago", format_duration(age_ms)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_keep_a_tenth_only_while_it_is_legible() {
        assert_eq!(format_duration(0), "0s");
        assert_eq!(format_duration(1_200), "1.2s");
        assert_eq!(format_duration(1_250), "1.3s");
        assert_eq!(format_duration(6_000), "6s");
        assert_eq!(format_duration(9_960), "10s");
        assert_eq!(format_duration(12_000), "12s");
        assert_eq!(format_duration(48_000), "48s");
    }

    #[test]
    fn durations_step_up_to_minutes_and_hours() {
        assert_eq!(format_duration(59_999), "59s");
        assert_eq!(format_duration(60_000), "1m");
        assert_eq!(format_duration(2_880_000), "48m");
        assert_eq!(format_duration(3_599_999), "59m");
        assert_eq!(format_duration(7_200_000), "2h");
    }

    #[test]
    fn token_counts_round_to_a_tenth_and_drop_it_when_whole() {
        assert_eq!(format_token_count(0), "0");
        assert_eq!(format_token_count(842), "842");
        assert_eq!(format_token_count(999), "999");
        assert_eq!(format_token_count(1_000), "1k");
        assert_eq!(format_token_count(3_140), "3.1k");
        assert_eq!(format_token_count(12_400), "12.4k");
        assert_eq!(format_token_count(84_000), "84k");
        assert_eq!(format_token_count(1_240_000), "1.2M");
    }

    #[test]
    fn structured_results_are_rendered_as_fields_never_as_prose() {
        assert_eq!(format_exit(0), "exit 0");
        assert_eq!(format_exit(101), "exit 101");
        assert_eq!(format_counter(0, 3), "1/3");
        assert_eq!(format_counter(2, 3), "3/3");
    }

    #[test]
    fn file_deltas_use_the_minus_sign() {
        assert_eq!(format_file_delta(36, 3), "+36 \u{2212}3");
        assert_eq!(format_files_changed(0, 0, 0), None);
        assert_eq!(
            format_files_changed(1, 2, 0).as_deref(),
            Some("1 file +2 \u{2212}0")
        );
        assert_eq!(
            format_files_changed(2, 36, 3).as_deref(),
            Some("2 files +36 \u{2212}3")
        );
    }

    #[test]
    fn the_footer_states_only_the_segments_a_harness_reported() {
        assert_eq!(
            turn_footer_segments(Some(12_400), Some(0.42), Some((2, 36, 3))),
            vec![
                SharedString::from("12.4k tokens"),
                SharedString::from("$0.42"),
                SharedString::from("2 files +36 \u{2212}3"),
            ]
        );
        // Codex reports no cost, and a turn that touched no file has no file clause.
        assert_eq!(
            turn_footer_segments(Some(3_100), None, Some((0, 0, 0))),
            vec![SharedString::from("3.1k tokens")]
        );
        assert!(turn_footer_segments(None, None, None).is_empty());
    }

    #[test]
    fn folds_and_lines_match_the_canvas_copy() {
        assert_eq!(format_worked(Some(22_000), 14), "worked 22s · 14 steps");
        assert_eq!(format_worked(Some(12_000), 1), "worked 12s · 1 step");
        assert_eq!(format_worked(None, 3), "worked");
        assert_eq!(format_stopped_after(12_000), "you stopped after 12s");
        assert_eq!(format_thought(12_000), "thought 12s");
        assert_eq!(format_working(0), "working 0s");
        assert_eq!(format_working(42_000), "working 42s");
        assert_eq!(format_working(72_000), "working 1m 12s");
        assert_eq!(format_working(3_900_000), "working 1h 5m");
        assert_eq!(
            format_retrying("rate limited", 12_000),
            "rate limited · retrying in 12s"
        );
        assert_eq!(
            format_compacted(84_000, Some(12_000)),
            "context compacted · 84k → 12k tokens"
        );
        assert_eq!(
            format_compacted(84_000, None),
            "context compacted · 84k tokens"
        );
        assert_eq!(format_resumed(7_200_000), "session resumed · 2h ago");
    }
}
