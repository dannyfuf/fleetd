//! The daemon surfaces of UX-SPEC §3.12: the cold-start splash, the "will not start" window
//! and the 28 px banner, plus the wording each of them is allowed to use.

use std::time::Instant;

use fleet_ui_kit::{Banner, DaemonSplash, DaemonState, Icon, KeyHintRow, Tone};
use gpui::{AnyElement, IntoElement, SharedString};

use crate::state::{
    AppState, DaemonLink, RECONNECT_BANNER_DWELL, RESTART_BANNER_DWELL, SPLASH_DETAIL_DELAY,
    reconnect_backoff,
};

/// The one sentence a reconnect after a daemon **restart** must say, verbatim (§3.12 D-17).
///
/// A warm banner that implies the agents came back is the single most damaging false
/// reassurance in the app: `ARCHITECTURE.md` is explicit that PTYs do not survive fleetd.
pub const RESTART_SENTENCE: &str =
    "fleetd restarted. Terminal sessions did not survive; worktrees, jobs and state are intact.";

/// What the 28 px banner under the context bar says right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BannerSpec {
    /// The sentence.
    pub text: String,
    /// The reconnect countdown, when one is running.
    pub countdown: Option<String>,
    /// Amber while reconnecting or after a restart; the dot carries the red.
    pub tone: Tone,
    /// The keys this banner owns while it is showing.
    pub hints: &'static [(&'static str, &'static str)],
}

/// The reconnect countdown of §3.12 C, cycling `3s → reconnecting… → 6s`.
#[must_use]
pub fn countdown_label(attempt: u32) -> String {
    format!("reconnecting in {}s", reconnect_backoff(attempt).as_secs())
}

/// The banner for the current daemon state, or `None` when there is nothing to say.
///
/// A dismissed `Lost` banner returns `None`; the daemon dot stays red, which is exactly what
/// §3.12 asks for.
#[must_use]
pub fn banner_spec(daemon: &DaemonLink, now: Instant) -> Option<BannerSpec> {
    match daemon {
        DaemonLink::Lost {
            attempt,
            dismissed: false,
        } => Some(BannerSpec {
            text: "fleetd stopped".to_owned(),
            countdown: Some(countdown_label(*attempt)),
            tone: Tone::Warning,
            hints: &[
                ("r", "reconnect now"),
                ("l", "open the log"),
                ("esc", "dismiss"),
            ],
        }),
        DaemonLink::Reconnected { restarted, since } => {
            let dwell = if *restarted {
                RESTART_BANNER_DWELL
            } else {
                RECONNECT_BANNER_DWELL
            };
            if now.saturating_duration_since(*since) >= dwell {
                return None;
            }
            Some(BannerSpec {
                text: if *restarted {
                    RESTART_SENTENCE.to_owned()
                } else {
                    "reconnected".to_owned()
                },
                countdown: None,
                tone: Tone::Warning,
                hints: &[],
            })
        }
        DaemonLink::Lost {
            dismissed: true, ..
        }
        | DaemonLink::Starting
        | DaemonLink::Failed { .. }
        | DaemonLink::Connected => None,
    }
}

/// The daemon dot's state (§2.2). Healthy is a bare dot; anything else grows a word.
#[must_use]
pub const fn dot_state(daemon: &DaemonLink) -> DaemonState {
    match daemon {
        DaemonLink::Connected | DaemonLink::Reconnected { .. } => DaemonState::Healthy,
        DaemonLink::Starting => DaemonState::Degraded,
        DaemonLink::Lost { .. } | DaemonLink::Failed { .. } => DaemonState::Lost,
    }
}

/// The word beside a degraded or lost dot.
#[must_use]
pub const fn dot_label(daemon: &DaemonLink) -> Option<&'static str> {
    match daemon {
        DaemonLink::Starting => Some("starting"),
        DaemonLink::Lost { .. } => Some("stopped"),
        DaemonLink::Failed { .. } => Some("down"),
        DaemonLink::Connected | DaemonLink::Reconnected { .. } => None,
    }
}

/// Renders the banner, or nothing.
#[must_use]
pub fn banner(daemon: &DaemonLink, now: Instant) -> Option<AnyElement> {
    let spec = banner_spec(daemon, now)?;
    let mut banner = Banner::warning(spec.text).icon(Icon::Unplug);
    if let Some(countdown) = spec.countdown {
        banner = banner.countdown(countdown);
    }
    if !spec.hints.is_empty() {
        let mut hints = KeyHintRow::new();
        for (key, label) in spec.hints {
            hints = hints.key(*key, *label);
        }
        banner = banner.hints(hints);
    }
    Some(banner.into_any_element())
}

/// The full-window daemon surface of §3.12 A and B, or `None` when the daemon is reachable.
#[must_use]
pub fn splash(state: &AppState, now: Instant) -> Option<AnyElement> {
    match &state.daemon {
        DaemonLink::Starting => {
            let mut splash = DaemonSplash::starting("Starting fleetd…");
            if now.saturating_duration_since(state.daemon_since) >= SPLASH_DETAIL_DELAY {
                splash = splash.detail(SharedString::from(
                    fleet_proto::paths::socket_path(&state.home)
                        .display()
                        .to_string(),
                ));
            }
            Some(splash.into_any_element())
        }
        DaemonLink::Failed {
            message,
            log_tail,
            stale_socket,
        } => {
            let mut splash = DaemonSplash::failed("fleetd could not start.")
                .log_lines(log_tail.iter().map(|line| SharedString::from(line.clone())))
                .hints(
                    KeyHintRow::new()
                        .key("r", "retry")
                        .key("L", "open log")
                        .key("D", "run doctor")
                        .key("ctrl-q", "quit"),
                );
            splash = splash.detail(if *stale_socket {
                SharedString::from(format!(
                    "The socket {} is stale.",
                    fleet_proto::paths::socket_path(&state.home).display()
                ))
            } else {
                SharedString::from(message.clone())
            });
            Some(splash.into_any_element())
        }
        DaemonLink::Connected | DaemonLink::Lost { .. } | DaemonLink::Reconnected { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn a_dismissed_banner_says_nothing_but_the_dot_stays_red() {
        let now = Instant::now();
        let lost = DaemonLink::Lost {
            attempt: 0,
            dismissed: true,
        };
        assert_eq!(banner_spec(&lost, now), None);
        assert_eq!(dot_state(&lost), DaemonState::Lost);
    }

    #[test]
    fn the_countdown_follows_the_backoff() {
        assert_eq!(countdown_label(0), "reconnecting in 1s");
        assert_eq!(countdown_label(1), "reconnecting in 2s");
        assert_eq!(countdown_label(2), "reconnecting in 4s");
        assert_eq!(countdown_label(9), "reconnecting in 8s");
    }

    #[test]
    fn a_restart_must_say_the_sessions_are_gone() {
        let now = Instant::now();
        let restarted = DaemonLink::Reconnected {
            restarted: true,
            since: now,
        };
        let spec = banner_spec(&restarted, now).unwrap_or_else(|| panic!("expected a banner"));
        assert_eq!(spec.text, RESTART_SENTENCE);
        assert!(spec.text.contains("did not survive"));

        // It stays for the full six seconds, not 800 ms.
        assert!(banner_spec(&restarted, now + Duration::from_secs(5)).is_some());
        assert!(banner_spec(&restarted, now + Duration::from_secs(7)).is_none());
    }

    #[test]
    fn a_dropped_connection_only_says_reconnected() {
        let now = Instant::now();
        let warm = DaemonLink::Reconnected {
            restarted: false,
            since: now,
        };
        let spec = banner_spec(&warm, now).unwrap_or_else(|| panic!("expected a banner"));
        assert_eq!(spec.text, "reconnected");
        assert!(banner_spec(&warm, now + Duration::from_millis(900)).is_none());
    }

    #[test]
    fn the_dot_is_bare_only_while_the_daemon_is_healthy() {
        assert_eq!(dot_state(&DaemonLink::Connected), DaemonState::Healthy);
        assert_eq!(dot_label(&DaemonLink::Connected), None);
        assert_eq!(dot_state(&DaemonLink::Starting), DaemonState::Degraded);
        assert_eq!(dot_label(&DaemonLink::Starting), Some("starting"));
    }
}
