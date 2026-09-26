//! Schedule times as the Schedules pane shows and sends them: local for the user, RFC 3339 in UTC
//! on the wire.

use super::*;

/// `every 15 min`, `every 2 h`, or `once · Sep 26 09:00`.
#[must_use]
pub(in crate::dialogs::board_settings) fn cadence_text(
    cadence: &Cadence,
    now: DateTime<Local>,
) -> String {
    match cadence {
        Cadence::Every { minutes } if *minutes >= 60 && minutes % 60 == 0 => {
            format!("every {} h", minutes / 60)
        }
        Cadence::Every { minutes } => format!("every {minutes} min"),
        Cadence::Once { at } => format!(
            "once \u{b7} {}",
            local_time(at, now).unwrap_or_else(|| at.clone())
        ),
    }
}

/// An RFC 3339 time in local time: `14:05` today, `Sep 26 09:00` any other day.
#[must_use]
pub(in crate::dialogs::board_settings) fn local_time(
    at: &str,
    now: DateTime<Local>,
) -> Option<String> {
    let local = parse_time(at)?.with_timezone(&Local);
    Some(if local.date_naive() == now.date_naive() {
        local.format("%H:%M").to_string()
    } else {
        local.format(DAY_FORMAT).to_string()
    })
}

/// When a run started, as the `Last runs` card says it: `13:50 today`, `09:00 yesterday`, or
/// `Sep 26 09:00`.
#[must_use]
pub(in crate::dialogs::board_settings) fn run_time(
    at: &str,
    now: DateTime<Local>,
) -> Option<String> {
    let local = parse_time(at)?.with_timezone(&Local);
    let today = now.date_naive();
    Some(if local.date_naive() == today {
        format!("{} today", local.format("%H:%M"))
    } else if today.pred_opt() == Some(local.date_naive()) {
        format!("{} yesterday", local.format("%H:%M"))
    } else {
        local.format(DAY_FORMAT).to_string()
    })
}

/// Parses an RFC 3339 time.
#[must_use]
pub(super) fn parse_time(at: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(at)
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

/// The start of the next hour, local, as the `Once at` row types it.
#[must_use]
pub(super) fn next_hour(now: DateTime<Local>) -> String {
    let next = now + chrono::Duration::hours(1);
    next.format("%Y-%m-%d %H:00").to_string()
}

/// A typed local time as the wire carries it: RFC 3339 in UTC, or the text as typed when it
/// is neither `YYYY-MM-DD HH:MM` nor already RFC 3339.
#[must_use]
pub(in crate::dialogs::board_settings) fn once_at_wire(text: &str) -> String {
    let text = text.trim();
    if let Some(at) = parse_time(text) {
        return at.to_rfc3339_opts(SecondsFormat::Secs, true);
    }
    NaiveDateTime::parse_from_str(text, ONCE_FORMAT)
        .ok()
        .and_then(|naive| Local.from_local_datetime(&naive).earliest())
        .map_or_else(
            || text.to_owned(),
            |at| {
                at.with_timezone(&Utc)
                    .to_rfc3339_opts(SecondsFormat::Secs, true)
            },
        )
}
