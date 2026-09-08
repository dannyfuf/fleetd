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

/// Formats the completed-turn file clause as `2 files changed +36 −3`, or nothing when the
/// turn touched no file.
#[must_use]
pub fn format_files_changed(files: usize, added: u64, removed: u64) -> Option<SharedString> {
    if files == 0 {
        return None;
    }
    let noun = if files == 1 { "file" } else { "files" };
    Some(SharedString::from(format!(
        "{files} {noun} changed {}",
        format_file_delta(added, removed)
    )))
}

/// The completed-turn footer metadata: `48s · 12.4k tokens · 2 files changed +36 −3`.
///
/// `[⏎] diff` and `[u] revert turn` are keycaps, so the footer row draws them itself.
#[must_use]
pub fn format_turn_footer(
    duration_ms: u64,
    tokens: u64,
    files: usize,
    added: u64,
    removed: u64,
) -> SharedString {
    let mut text = format!(
        "{} · {} tokens",
        format_duration(duration_ms),
        format_token_count(tokens)
    );
    if let Some(files) = format_files_changed(files, added, removed) {
        text.push_str(" · ");
        text.push_str(&files);
    }
    SharedString::from(text)
}

/// The successful-work fold: `worked 12s · 3 tool calls`.
#[must_use]
pub fn format_worked(duration_ms: u64, tool_calls: usize) -> SharedString {
    let noun = if tool_calls == 1 {
        "tool call"
    } else {
        "tool calls"
    };
    SharedString::from(format!(
        "worked {} · {tool_calls} {noun}",
        format_duration(duration_ms)
    ))
}

/// The collapsed reasoning line: `thinking · 6s`.
#[must_use]
pub fn format_thinking(duration_ms: u64) -> SharedString {
    SharedString::from(format!("thinking · {}", format_duration(duration_ms)))
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
    fn file_deltas_use_the_minus_sign() {
        assert_eq!(format_file_delta(36, 3), "+36 \u{2212}3");
        assert_eq!(format_files_changed(0, 0, 0), None);
        assert_eq!(
            format_files_changed(1, 2, 0).as_deref(),
            Some("1 file changed +2 \u{2212}0")
        );
        assert_eq!(
            format_files_changed(2, 36, 3).as_deref(),
            Some("2 files changed +36 \u{2212}3")
        );
    }

    #[test]
    fn the_footer_matches_the_canvas_line() {
        assert_eq!(
            format_turn_footer(48_000, 12_400, 2, 36, 3),
            "48s · 12.4k tokens · 2 files changed +36 \u{2212}3"
        );
        assert_eq!(
            format_turn_footer(48_000, 12_400, 0, 0, 0),
            "48s · 12.4k tokens"
        );
    }

    #[test]
    fn folds_and_lines_match_the_canvas_copy() {
        assert_eq!(format_worked(12_000, 3), "worked 12s · 3 tool calls");
        assert_eq!(format_worked(12_000, 1), "worked 12s · 1 tool call");
        assert_eq!(format_thinking(6_000), "thinking · 6s");
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
